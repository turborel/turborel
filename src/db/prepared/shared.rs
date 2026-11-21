//! \[DESIGN\] What prepared statements keep, shared by every session of a
//! database (RULES.md D610): a statement prepared alike in two sessions,
//! of one text, with one collation and its parameters of the same types,
//! finds one place for what it keeps. A plan kept there is taken by each
//! execution whose definitions, authorization, schema, time zone and
//! settings are those it was kept with (`plans.rs`'s `Key`), so it is
//! dropped by a change of the definitions or privileges, and a learned
//! plan by its tables' rows moving (`moved`).
//!
//! The places are found by an XXH3 hash of what the statements were
//! prepared alike in, and that is compared whole, so two statements of one
//! hash share no place unless they are alike. A place is held by the
//! statements prepared with it: once none is, it is gone, and its entry is
//! cleared as its hash is next found.

use std::sync::{Arc, Mutex, RwLock, Weak};

use super::plans::Kept;
use crate::hash::xxhash::{Xxh3Map, xxh3_64};
use crate::sync::lock;

/// What a statement prepared alike keeps, and what it was prepared alike
/// in.
#[derive(Debug, Default)]
pub(in crate::db) struct Place {
    alike: String,
    pub(super) kept: RwLock<Option<Kept>>,
}

/// The places of a database's prepared statements, by the hash of what
/// each was prepared alike in.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    places: Mutex<Xxh3Map<u64, Vec<Weak<Place>>>>,
}

impl Shared {
    /// The place of a statement prepared alike in `alike`: found, or made
    /// for it, if none is held.
    pub(in crate::db) fn place(&self, alike: String) -> Arc<Place> {
        let Ok(mut places) = lock(&self.places) else { return Arc::new(Place { alike, kept: RwLock::default() }) };
        let found = places.entry(xxh3_64(alike.as_bytes())).or_default();
        found.retain(|place| place.strong_count() > 0);
        if let Some(place) = found.iter().filter_map(Weak::upgrade).find(|place| place.alike == alike) {
            return place;
        }
        let place = Arc::new(Place { alike, kept: RwLock::default() });
        found.push(Arc::downgrade(&place));
        place
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D610: statements prepared alike share a
    /// place, others not, and a place none holds is gone. MC/DC of finding
    /// one: alike or not, held or not.
    #[test]
    fn statements_prepared_alike_share_a_place_while_one_is_held() {
        let shared = Shared::default();
        let first = shared.place("SELECT 1".to_owned());
        let again = shared.place("SELECT 1".to_owned());
        assert!(Arc::ptr_eq(&first, &again), "alike, held");
        let other = shared.place("SELECT 2".to_owned());
        assert!(!Arc::ptr_eq(&first, &other), "not alike");
        let weak = Arc::downgrade(&first);
        drop((first, again));
        assert!(weak.upgrade().is_none());
        let made = shared.place("SELECT 1".to_owned());
        assert_eq!(made.alike, "SELECT 1", "made again once none held it");
        let places = lock(&shared.places).unwrap();
        assert_eq!(places.get(&xxh3_64(b"SELECT 1")).map(Vec::len), Some(1), "the gone one cleared");
    }
}
